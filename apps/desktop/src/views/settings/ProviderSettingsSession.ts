import {
  defaultConnection,
  connectionDraftChanged,
  getProviderSettings,
  newConnectionId,
  connectionLabel,
  removeProviderConnection,
  removeProviderCredential,
  retryProviderOperation,
  saveProviderConnection,
  switchProviderConnection,
  testProviderConnection,
  type Connection,
  type ConnectionDraft,
  type ProviderSettings,
  type SmartCheckProvider,
  type PendingProviderOperation,
} from "../../lib/smartCheckProviders"
import { removeTypeSafeApiKey } from "../../lib/checkAvailability"
import { confirm } from "@tauri-apps/plugin-dialog"
import type { SettingsControlId } from "../../lib/settingsSearchTargets"

type DraftState = {
  draft: ConnectionDraft
  tested: boolean
}

function draftState(id: string, connection: Connection): DraftState {
  return { draft: { connection_id: id, connection, credential: null }, tested: false }
}
type Snapshot = {
  loading: boolean
  action: "test" | "save" | "remove" | "switch" | "delete" | "retry" | null
  editor: "add" | "edit" | null
  saved: ProviderSettings | null
  selectedId: string
  drafts: Record<string, DraftState>
  busy: boolean
  error: string | null
  errorLocation: "editor" | "connections"
  legacyKeyForgotten: boolean
  status: string | null
  activeConfirmed: boolean
}

const safeErrors = new Set([
  "Enter a TypeSafe API key.",
  "Enter a Cloudflare API token.",
  "Credential storage is unavailable.",
  "The saved credential is missing. Use Edit → Change to add it again.",
  "The saved credential has an invalid format. Use Edit → Change to replace it.",
  "The system did not allow credential access. Check app access and retry.",
  "Credential access was cancelled. Use the connection again when ready.",
  "Credential storage matched more than one entry. Check the system credential store before retrying.",
  "Could not read the saved credential. Retry, or use Edit → Change to replace it.",
  "Could not save the connection credential.",
  "Could not access secure storage. Unlock the system credential store and retry.",
  "Secure storage rejected the credential write. Check app access and retry.",
  "Could not save the API token to secure storage. Check credential storage access and retry.",
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
  "Could not cancel unfinished Burn Checks. Checks are paused. Retry the connection change.",
  "Could not pause YAML Burn Checks.",
  "Could not enable YAML Burn Checks.",
  "Could not confirm the saved credential. Checks are paused. Retry the connection update in Settings → Checks.",
  "Could not restore the previous credential. Checks are paused. Retry the connection update in Settings → Checks.",
  "Could not restore the previous connection state. Retry the connection update in Settings → Checks.",
  "Could not restore the previous connection. Checks are paused. Retry the connection update in Settings → Checks.",
  "A connection update is incomplete. Retry it in Settings → Checks before removing a credential.",
  "The active model provider credential is missing. Update it in Settings → Checks.",
  "This connection has no supported model profile or runner. Choose a supported provider and model.",
  "The selected provider did not report usable model limits.",
  "Could not read model limits from the selected provider. Check the connection and try again.",
  "The connection revision limit was reached. Save a new connection.",
  "The connection changed. Reload it and try again.",
  "The connection already exists. Add a new connection.",
  "Could not remove the connection.",
  "This connection changed in another window. Close the editor and refresh Settings.",
  "This connection changed in another window. Refresh Settings before removing it.",
  "The saved credential reference changed. Close the editor and refresh Settings.",
  "This connection identity was already used. Open Add connection again.",
  "This connection was removed. Close the editor and refresh Settings.",
  "The saved TypeSafe key can be selected only for Jev.",
  "Add a new connection to use a different provider.",
  "Saved connection display numbers are invalid.",
  "The connection display number limit was reached.",
  "This recovery operation is no longer pending. Refresh Settings.",
  "Could not finish credential cleanup. Use Retry in Settings → Checks.",
  "Could not complete the connection operation. Use Retry in Settings → Checks.",
  "A connection operation is pending. Use Retry in Settings → Checks.",
  "Could not save connection settings. Use Retry in Settings → Checks.",
  "The Save request identity is invalid. Reopen the connection editor.",
  "The saved connection operation does not match the saved settings.",
  "The selected provider needs a credential. Retry after restoring secure storage.",
])

export const ACTIVE_CONNECTION_UNCONFIRMED =
  "Could not confirm the active connection. Retry loading connections."

export function providerErrorMessage(error: unknown, fallback: string): string {
  const message = error instanceof Error ? error.message : error
  return typeof message === "string" && safeErrors.has(message) ? message : fallback
}

export function pendingOperationMessage(
  operation: PendingProviderOperation,
  editorOpen = false,
): string {
  const action =
    operation.kind === "save" ? "save" : operation.kind === "switch" ? "selection" : "removal"
  return `The connection ${action} is not finished. Choose Retry to resolve it. This may restore the previous settings. Closing ${editorOpen ? "this dialog" : "Settings"} does not cancel it.`
}

const providerFieldTargets: Array<{
  control: SettingsControlId
  provider: SmartCheckProvider
}> = [
  { control: "typeSafeApiKey", provider: "jev" },
  { control: "ollamaUrl", provider: "ollama" },
  { control: "ollamaModel", provider: "ollama" },
  { control: "ollamaApiKey", provider: "ollama" },
  { control: "cloudflareAccountId", provider: "cloudflare" },
  { control: "cloudflareApiToken", provider: "cloudflare" },
  { control: "cloudflareModel", provider: "cloudflare" },
  { control: "customUrl", provider: "custom" },
  { control: "customModel", provider: "custom" },
  { control: "customApiKey", provider: "custom" },
  { control: "customResponseMode", provider: "custom" },
]

export function providerFieldTarget(control: string | null | undefined) {
  return providerFieldTargets.find((target) => target.control === control)
}

export class ProviderSettingsSession {
  private saveSubmissions = new Map<
    string,
    { connectionId: string; editor: "add" | "edit"; draft: ConnectionDraft }
  >()
  private createdDraftIds = new Set<string>()
  private legacyKeyRemovalProfiles: Record<string, Connection> | null = null
  private readonly onSelectionChange: ((selected: boolean) => void) | undefined
  private readonly onPendingChange: ((pending: boolean) => void) | undefined
  constructor(
    onSelectionChange?: ((selected: boolean) => void) | undefined,
    onPendingChange?: ((pending: boolean) => void) | undefined,
  ) {
    this.onSelectionChange = onSelectionChange
    this.onPendingChange = onPendingChange
  }
  private listeners = new Set<() => void>()
  private generation = 0
  private providerSelections = new Map<SmartCheckProvider, string>()
  private targetKey: string | null = null
  private targetProvider: SmartCheckProvider | null = null
  private targetAdd = false
  private state: Snapshot = {
    loading: true,
    action: null,
    editor: null,
    saved: null,
    selectedId: "jev",
    drafts: {},
    busy: false,
    error: null,
    errorLocation: "connections",
    legacyKeyForgotten: false,
    status: null,
    activeConfirmed: false,
  }
  getSnapshot = () => this.state
  subscribe = (listener: () => void) => {
    this.listeners.add(listener)
    if (this.listeners.size === 1) {
      void this.refresh()
    }
    return () => {
      this.listeners.delete(listener)
      if (!this.listeners.size) this.generation++
    }
  }
  refresh = async () => {
    if (this.state.busy || (this.state.loading && this.state.saved)) return
    const generation = ++this.generation
    this.publish({ loading: true, error: null })
    try {
      const saved = await getProviderSettings()
      if (generation !== this.generation) return
      this.validate(saved)
      this.load(saved)
    } catch (error) {
      if (generation === this.generation)
        this.publish({
          activeConfirmed: false,
          error: providerErrorMessage(
            error,
            "Could not load saved connections. Retry loading them.",
          ),
        })
    } finally {
      if (generation === this.generation) {
        this.publish({ loading: false })
        this.revealPendingTarget()
      }
    }
  }
  private publish(update: Partial<Snapshot>) {
    this.state = { ...this.state, ...update }
    this.listeners.forEach((listener) => listener())
    if ("saved" in update || "activeConfirmed" in update)
      this.onSelectionChange?.(
        Boolean(
          this.state.activeConfirmed &&
          this.state.saved?.activeId &&
          !this.state.saved.pendingOperation,
        ),
      )
    if ("saved" in update) this.onPendingChange?.(Boolean(this.state.saved?.pendingOperation))
  }
  private validate(saved: ProviderSettings) {
    if (
      !saved ||
      !saved.profiles ||
      !saved.displayNumbers ||
      typeof saved.displayNumbers !== "object" ||
      Array.isArray(saved.displayNumbers) ||
      !("pendingOperation" in saved) ||
      (saved.activeId !== null && !saved.profiles[saved.activeId])
    )
      throw new Error("Missing active connection")
    const used = new Set<string>()
    for (const [id, profile] of Object.entries(saved.profiles)) {
      const number = saved.displayNumbers[id]
      const key = `${profile.provider}:${number}`
      if (!Number.isSafeInteger(number) || !number || number < 1 || used.has(key))
        throw new Error("Invalid connection display numbers")
      used.add(key)
    }
    const operation = saved.pendingOperation
    if (
      operation !== null &&
      (!operation ||
        typeof operation.id !== "string" ||
        !operation.id ||
        !["save", "switch", "remove"].includes(operation.kind) ||
        typeof operation.connectionId !== "string" ||
        !operation.connectionId ||
        !(
          operation.requestId === null ||
          (typeof operation.requestId === "string" && operation.requestId.length > 0)
        ))
    )
      throw new Error("Invalid pending connection operation")
    if (operation && operation.kind !== "save" && operation.requestId !== null)
      throw new Error("Invalid pending connection request identity")
  }
  private clearSaveSubmissions(connectionId: string) {
    for (const [requestId, submission] of this.saveSubmissions) {
      if (submission.connectionId === connectionId) this.saveSubmissions.delete(requestId)
    }
  }
  private savedDrafts(
    saved: ProviderSettings,
    pruneRemoved = false,
  ): Record<string, DraftState> {
    const drafts = { ...this.state.drafts }
    const removedKeyProfiles = this.legacyKeyRemovalProfiles
    this.legacyKeyRemovalProfiles = null
    for (const [id, previous] of Object.entries(removedKeyProfiles ?? {})) {
      const current = drafts[id]
      const connection = current?.draft.connection
      const updated = saved.profiles[id]
      if (
        !current ||
        !connection ||
        !updated ||
        previous.credential?.kind !== "legacy_type_safe" ||
        connection.provider !== previous.provider ||
        connection.revision !== previous.revision ||
        JSON.stringify(connection.credential) !== JSON.stringify(previous.credential) ||
        JSON.stringify(updated) !==
          JSON.stringify({ ...previous, revision: previous.revision + 1 })
      )
        continue
      drafts[id] = {
        draft: {
          ...current.draft,
          connection: { ...connection, revision: updated.revision },
        },
        tested: false,
      }
    }
    for (const id of Object.keys(drafts)) {
      if (
        this.state.saved?.profiles[id] &&
        !saved.profiles[id] &&
        (pruneRemoved ||
          !connectionDraftChanged(drafts[id]!.draft, this.state.saved.profiles[id]))
      )
        delete drafts[id]
    }
    for (const [id, connection] of Object.entries(saved.profiles)) {
      const current = drafts[id]
      if (
        current &&
        (connectionDraftChanged(current.draft, this.state.saved?.profiles[id]) ||
          (this.state.editor && saved.pendingOperation?.connectionId === id))
      )
        continue
      if (!current || JSON.stringify(current.draft.connection) !== JSON.stringify(connection)) {
        drafts[id] = draftState(id, connection)
      }
    }
    return drafts
  }
  private load(saved: ProviderSettings, clearError = true, pruneRemoved = false) {
    this.validate(saved)
    const drafts = this.savedDrafts(saved, pruneRemoved)
    for (const [provider, id] of this.providerSelections) {
      if (!drafts[id]) this.providerSelections.delete(provider)
    }
    const selectedId = drafts[this.state.selectedId]
      ? this.state.selectedId
      : (saved.activeId ?? Object.keys(saved.profiles)[0] ?? "")
    if (drafts[selectedId])
      this.providerSelections.set(drafts[selectedId].draft.connection.provider, selectedId)
    this.publish({
      saved,
      selectedId,
      drafts,
      activeConfirmed: true,
      ...(clearError ? { error: null } : {}),
    })
  }
  select(id: string) {
    if (this.state.busy || this.state.loading || !this.state.drafts[id]) return
    this.providerSelections.set(this.state.drafts[id]!.draft.connection.provider, id)
    this.publish({
      selectedId: id,
      error: null,
      status: null,
    })
  }
  async useConnection(id: string) {
    if (
      this.state.busy ||
      this.state.loading ||
      this.state.saved?.pendingOperation ||
      !this.state.saved?.profiles[id]
    )
      return
    this.select(id)
    const current = this.state.drafts[id]
    if (!current) return
    if (connectionDraftChanged(current.draft, this.state.saved.profiles[id])) {
      this.publish({
        editor: "edit",
        status: "Save or discard your changes before using this connection.",
      })
      return
    }
    await this.run("switch")
  }
  openEdit(id = this.state.saved?.activeId) {
    if (!id || this.state.busy || this.state.loading || !this.state.saved?.profiles[id]) return
    this.select(id)
    this.publish({ editor: "edit" })
  }
  add(provider: SmartCheckProvider = "jev") {
    if (this.state.busy || this.state.loading || !this.state.saved) return
    const retained = this.state.drafts[this.state.selectedId]
    if (
      retained &&
      this.createdDraftIds.has(this.state.selectedId) &&
      !this.state.saved.profiles[this.state.selectedId] &&
      retained.draft.connection.provider === provider
    ) {
      this.publish({ editor: "add", error: null, status: null })
      return
    }
    const id = newConnectionId(
      provider,
      Object.keys(this.state.drafts).filter(
        (id) => this.state.drafts[id]!.draft.connection.provider === provider,
      ),
    )
    this.createdDraftIds.add(id)
    this.providerSelections.set(provider, id)
    this.publish({
      drafts: {
        ...this.state.drafts,
        [id]: draftState(id, defaultConnection(provider)),
      },
      selectedId: id,
      editor: "add",
      error: null,
      status: null,
    })
  }
  private hasChanges() {
    return Object.entries(this.state.drafts).some(([id, current]) =>
      connectionDraftChanged(
        current.draft,
        this.state.saved?.profiles[id] ?? defaultConnection(current.draft.connection.provider),
      ),
    )
  }
  async closeEditor() {
    if (this.state.busy) return
    if (this.state.saved?.pendingOperation) {
      this.publish({
        editor: null,
        status:
          "The connection operation is still pending. Your draft stays on this device until you discard it or finish recovery.",
      })
      return
    }
    if (this.hasChanges() && !(await this.confirmDiscard())) return
    this.saveSubmissions.clear()
    this.createdDraftIds.clear()
    const saved = this.state.saved
    this.publish({
      drafts: Object.fromEntries(
        Object.entries(saved?.profiles ?? {}).map(([id, connection]) => [
          id,
          draftState(id, connection),
        ]),
      ),
      selectedId: saved?.activeId ?? Object.keys(saved?.profiles ?? {})[0] ?? "",
      editor: null,
      error: null,
      status: null,
    })
  }
  private confirmDiscard() {
    return confirm("Discard your unsaved connection changes?", {
      title: "Discard changes",
      kind: "warning",
      okLabel: "Discard changes",
      cancelLabel: "Keep editing",
    })
  }
  async changeAddProvider(provider: SmartCheckProvider) {
    if (this.state.editor !== "add" || this.state.busy || this.state.loading) return
    const id = this.state.selectedId
    const current = this.state.drafts[id]
    if (!current || current.draft.connection.provider === provider) return
    if (
      connectionDraftChanged(
        current.draft,
        defaultConnection(current.draft.connection.provider),
      ) &&
      !(await this.confirmDiscard())
    )
      return
    if (
      this.state.editor !== "add" ||
      this.state.busy ||
      this.state.loading ||
      this.state.selectedId !== id ||
      this.state.drafts[id] !== current ||
      !this.createdDraftIds.has(id) ||
      this.state.saved?.profiles[id]
    )
      return
    const drafts = { ...this.state.drafts }
    delete drafts[id]
    this.createdDraftIds.delete(id)
    this.publish({ drafts })
    this.add(provider)
  }
  revealTarget(control: string | null | undefined, revision: number | undefined) {
    const key = `${control ?? ""}:${revision ?? 0}`
    if (this.targetKey === key) return
    this.targetKey = key
    this.targetAdd = control === "decisionProvider"
    this.targetProvider = providerFieldTarget(control)?.provider ?? null
    this.revealPendingTarget()
  }
  private revealPendingTarget() {
    if (this.targetAdd && this.state.saved && !this.state.loading && !this.state.busy) {
      this.targetAdd = false
      if (this.state.editor !== "add") this.add()
      return
    }
    if (this.targetProvider && this.state.saved && !this.state.loading && !this.state.busy) {
      this.selectProvider(this.targetProvider)
      this.publish({
        editor: this.createdDraftIds.has(this.state.selectedId) ? "add" : "edit",
      })
      this.targetProvider = null
    }
  }
  selectProvider(provider: SmartCheckProvider) {
    if (this.state.busy || this.state.loading || !this.state.saved) return
    const remembered = this.providerSelections.get(provider)
    const id =
      (remembered && this.state.drafts[remembered] ? remembered : undefined) ??
      Object.entries(this.state.drafts).find(
        ([, value]) => value.draft.connection.provider === provider,
      )?.[0] ??
      newConnectionId(
        provider,
        Object.keys(this.state.drafts).filter(
          (id) => this.state.drafts[id]!.draft.connection.provider === provider,
        ),
      )
    if (!this.state.drafts[id]) {
      this.createdDraftIds.add(id)
      this.publish({
        drafts: {
          ...this.state.drafts,
          [id]: draftState(id, defaultConnection(provider)),
        },
      })
    }
    this.providerSelections.set(provider, id)
    this.publish({
      selectedId: id,
      error: null,
      status: null,
    })
  }
  edit(connection: Connection, credential: string | null) {
    if (this.state.busy || this.state.loading || !this.state.saved) return
    const id = this.state.selectedId
    this.publish({
      drafts: {
        ...this.state.drafts,
        [id]: {
          draft: { connection_id: id, connection, credential },
          tested: false,
        },
      },
      error: null,
      status: null,
    })
  }
  private acceptSaved(saved: ProviderSettings, id: string, status: string) {
    this.validate(saved)
    const connection = saved.profiles[id]
    if (!connection) throw new Error("Missing saved connection")
    this.createdDraftIds.delete(id)
    this.publish({
      saved,
      activeConfirmed: true,
      drafts: {
        ...this.savedDrafts(saved),
        [id]: draftState(id, connection),
      },
      status,
    })
  }
  useLegacyKey() {
    const current = this.state.drafts[this.state.selectedId]
    if (!current || this.state.busy || this.state.legacyKeyForgotten) return
    this.edit({ ...current.draft.connection, credential: { kind: "legacy_type_safe" } }, null)
  }
  async forgetLegacyKey() {
    const saved = this.state.saved
    if (
      this.state.busy ||
      this.state.loading ||
      this.state.legacyKeyForgotten ||
      !saved ||
      saved.pendingOperation
    )
      return
    this.publish({ busy: true, action: "remove", error: null, errorLocation: "editor" })
    try {
      if (!(await this.confirmForgetSharedKey())) return
      await this.removeSharedKey(saved)
      this.load(await getProviderSettings())
      const current = this.state.drafts[this.state.selectedId]
      if (current?.draft.connection.credential?.kind === "legacy_type_safe")
        this.publish({
          drafts: {
            ...this.state.drafts,
            [this.state.selectedId]: {
              ...current,
              draft: {
                ...current.draft,
                connection: { ...current.draft.connection, credential: null },
              },
              tested: false,
            },
          },
        })
      this.publish({ status: "Shared TypeSafe key forgotten." })
    } catch (error) {
      this.publish({
        error: providerErrorMessage(error, "Could not forget the saved key. Try again."),
      })
      await this.readAfterError()
    } finally {
      this.finishAction()
    }
  }
  private confirmForgetSharedKey() {
    return confirm("Forget this key? Other connections that use it will stop working.", {
      title: "Forget shared TypeSafe key",
      kind: "warning",
      okLabel: "Forget key",
      cancelLabel: "Cancel",
    })
  }
  private async removeSharedKey(saved: ProviderSettings) {
    await removeTypeSafeApiKey()
    this.legacyKeyRemovalProfiles = saved.profiles
    this.publish({ legacyKeyForgotten: true })
  }
  async removeConnection(id: string) {
    const saved = this.state.saved
    const connection = saved?.profiles[id]
    if (
      !saved ||
      !connection ||
      saved.pendingOperation ||
      this.state.busy ||
      this.state.loading
    )
      return false
    this.publish({
      busy: true,
      action: "delete",
      error: null,
      status: null,
      errorLocation: "connections",
    })
    try {
      const selected = saved.activeId === id
      const label = connectionLabel(id, connection, saved)
      if (
        !(await confirm(
          selected
            ? `Remove ${label}? This pauses model-based checks and clears the selected connection. Saved results and model usage stay on this device.`
            : `Remove ${label}? This does not pause checks. Saved results and model usage stay on this device.`,
          {
            title: "Remove connection",
            kind: "warning",
            okLabel: "Remove connection",
            cancelLabel: "Cancel",
          },
        ))
      )
        return false
      this.load(await removeProviderConnection(id, connection.revision), true, true)
      this.publish({ status: "Connection removed." })
      return true
    } catch (error) {
      return await this.reportConnectionError(
        error,
        "Could not remove the connection. Reload connections and try again.",
      )
    } finally {
      this.finishAction()
    }
  }
  private async readAfterError() {
    try {
      const saved = await getProviderSettings()
      this.load(saved, false)
    } catch {
      this.publish({ activeConfirmed: false, status: ACTIVE_CONNECTION_UNCONFIRMED })
    }
  }
  private async reportConnectionError(error: unknown, message: string) {
    this.publish({ error: providerErrorMessage(error, message) })
    await this.readAfterError()
    return false
  }
  private finishAction() {
    this.publish({ busy: false, action: null })
    this.revealPendingTarget()
  }
  async run(action: "test" | "save" | "remove" | "switch") {
    if (
      this.state.busy ||
      this.state.loading ||
      !this.state.saved ||
      this.state.saved.pendingOperation
    )
      return
    const savedBefore = this.state.saved
    const id = this.state.selectedId
    let current = this.state.drafts[id]
    if (!current) return
    const savedConnection = savedBefore.profiles[id]
    const intent = this.createdDraftIds.has(id) ? "create" : "update"
    if (
      action === "save" &&
      this.state.editor === "add" &&
      (intent !== "create" || savedConnection)
    ) {
      this.publish({
        error: "This connection identity was already used. Open Add connection again.",
        errorLocation: "editor",
      })
      return
    }
    const sharedKey = savedConnection?.credential?.kind === "legacy_type_safe"
    if (action === "remove" && sharedKey && this.state.legacyKeyForgotten) return
    if (
      action === "switch" &&
      (!savedConnection ||
        this.state.saved.activeId === id ||
        connectionDraftChanged(current.draft, savedConnection))
    )
      return
    if (action === "remove" && !savedConnection?.credential) return
    if (action === "test") current = { ...current, tested: false }
    this.publish({
      busy: true,
      action,
      error: null,
      errorLocation: action === "switch" ? "connections" : "editor",
      status: null,
      drafts: { ...this.state.drafts, [id]: current },
    })
    try {
      if (action === "test") {
        await testProviderConnection(current.draft)
        this.publish({
          drafts: { ...this.state.drafts, [id]: { ...current, tested: true } },
        })
      } else if (action === "save" || action === "switch") {
        const requestId = action === "save" ? crypto.randomUUID() : null
        if (requestId)
          this.saveSubmissions.set(requestId, {
            connectionId: id,
            editor: intent === "create" ? "add" : "edit",
            draft: current.draft,
          })
        const saved =
          action === "save"
            ? await saveProviderConnection(current.draft, intent, requestId!)
            : await switchProviderConnection(id)
        this.acceptSaved(
          saved,
          id,
          action === "save" ? "Connection saved." : "Connection selected.",
        )
        if (requestId) this.clearSaveSubmissions(id)
        if (action === "save") this.publish({ editor: null })
      } else {
        if (sharedKey && !(await this.confirmForgetSharedKey())) return
        this.publish({
          drafts: {
            ...this.state.drafts,
            [id]: { ...current, tested: false },
          },
        })
        if (sharedKey) {
          await this.removeSharedKey(savedBefore)
        } else await removeProviderCredential(id)
        const saved = await getProviderSettings()
        this.validate(saved)
        const connection = saved.profiles[id]
        if (!connection) throw new Error("The saved connection is missing.")
        const drafts = this.savedDrafts(saved)
        if (!sharedKey)
          drafts[id] = {
            draft: {
              ...current.draft,
              connection: {
                ...current.draft.connection,
                credential: connection.credential,
                revision: connection.revision,
              },
              credential: current.draft.credential,
            },
            tested: false,
          }
        this.publish({
          saved,
          activeConfirmed: true,
          drafts,
          status: sharedKey
            ? "Shared TypeSafe key forgotten."
            : "Saved credential forgotten. The connection remains saved.",
        })
      }
    } catch (error) {
      this.publish({
        error: providerErrorMessage(
          error,
          `Could not ${action === "remove" ? "remove the credential" : action === "switch" ? "select the saved connection" : `${action} the connection`}. Check the settings and try again.`,
        ),
      })
      if (action === "save" || action === "switch" || action === "remove") {
        await this.readAfterError()
      }
    } finally {
      this.finishAction()
    }
  }
  async retryPendingOperation() {
    const operation = this.state.saved?.pendingOperation
    if (!operation || this.state.busy || this.state.loading) return false
    this.publish({
      busy: true,
      action: "retry",
      error: null,
      status: null,
      errorLocation: this.state.editor ? "editor" : "connections",
    })
    const submission =
      operation.kind === "save" && operation.requestId
        ? this.saveSubmissions.get(operation.requestId)
        : undefined
    const ownsSave = submission?.connectionId === operation.connectionId
    try {
      const result = await retryProviderOperation(operation.id)
      this.validate(result.settings)
      if (
        result.settings.pendingOperation ||
        !["completed", "rolled_back"].includes(result.outcome)
      )
        throw new Error("Invalid connection recovery result")
      this.load(result.settings)
      const current = this.state.drafts[operation.connectionId]
      const unchanged =
        ownsSave &&
        current &&
        JSON.stringify(current.draft) === JSON.stringify(submission.draft)
      const connection = result.settings.profiles[operation.connectionId]
      if (unchanged && result.outcome === "completed") {
        this.acceptSaved(result.settings, operation.connectionId, "Connection saved.")
        if (this.state.selectedId === operation.connectionId) this.publish({ editor: null })
      } else if (
        ownsSave &&
        current &&
        connection &&
        result.outcome === "completed" &&
        current.draft.connection_id === submission.draft.connection_id &&
        current.draft.connection.provider === submission.draft.connection.provider &&
        connection.provider === submission.draft.connection.provider &&
        current.draft.connection.revision === submission.draft.connection.revision &&
        JSON.stringify(current.draft.connection.credential) ===
          JSON.stringify(submission.draft.connection.credential)
      ) {
        this.createdDraftIds.delete(operation.connectionId)
        this.publish({
          drafts: {
            ...this.state.drafts,
            [operation.connectionId]: {
              ...current,
              draft: {
                ...current.draft,
                connection: {
                  ...current.draft.connection,
                  revision: connection.revision,
                  credential: connection.credential,
                },
              },
              tested: false,
            },
          },
          ...(this.state.editor && this.state.selectedId === operation.connectionId
            ? { editor: "edit" }
            : {}),
        })
      } else if (unchanged && result.outcome === "rolled_back") {
        const previous = result.settings.profiles[operation.connectionId]
        this.publish({
          drafts: {
            ...this.state.drafts,
            [operation.connectionId]: {
              ...current,
              draft: {
                ...current.draft,
                connection: previous
                  ? {
                      ...current.draft.connection,
                      revision: previous.revision,
                      credential: previous.credential,
                    }
                  : current.draft.connection,
              },
              tested: false,
            },
          },
          ...(this.state.editor && this.state.selectedId === operation.connectionId
            ? { editor: submission.editor }
            : {}),
        })
      }
      if (!(unchanged && result.outcome === "completed"))
        this.publish({
          status:
            result.outcome === "completed"
              ? "The pending connection operation completed. Your unsaved draft has not changed."
              : "The connection operation was rolled back. Your draft has not been saved.",
        })
      if (ownsSave) this.clearSaveSubmissions(operation.connectionId)
      return true
    } catch (error) {
      return await this.reportConnectionError(
        error,
        "Could not retry the connection operation. Reload connections and try again.",
      )
    } finally {
      this.finishAction()
    }
  }
}
