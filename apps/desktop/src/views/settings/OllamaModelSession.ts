import {
  getInstalledOllamaModels,
  type ConnectionDraft,
  type InstalledOllamaModel,
} from "../../lib/smartCheckProviders"
import { providerErrorMessage } from "./ProviderSettingsSession"

type Snapshot = {
  models: InstalledOllamaModel[] | null
  loading: boolean
  error: string | null
}

function sameServer(left: ConnectionDraft | undefined, right: ConnectionDraft | undefined) {
  return Boolean(
    left &&
    right &&
    left.connection_id === right.connection_id &&
    left.connection.revision === right.connection.revision &&
    left.connection.provider === right.connection.provider &&
    JSON.stringify(left.connection.endpoint) === JSON.stringify(right.connection.endpoint) &&
    JSON.stringify(left.connection.credential) ===
      JSON.stringify(right.connection.credential) &&
    left.credential === right.credential,
  )
}

export class OllamaModelSession {
  private listeners = new Set<() => void>()
  private generation = 0
  private boundDraft: ConnectionDraft | undefined
  private state: Snapshot = { models: null, loading: false, error: null }
  private readonly getDraft: () => ConnectionDraft | undefined

  constructor(getDraft: () => ConnectionDraft | undefined) {
    this.getDraft = getDraft
  }

  getSnapshot = () => this.state
  subscribe = (listener: () => void) => {
    this.listeners.add(listener)
    return () => {
      this.listeners.delete(listener)
      if (!this.listeners.size) {
        this.generation++
        this.boundDraft = undefined
        this.state = { models: null, loading: false, error: null }
      }
    }
  }
  private publish(update: Partial<Snapshot>) {
    this.state = { ...this.state, ...update }
    this.listeners.forEach((listener) => listener())
  }
  bind = (draft = this.getDraft()) => {
    if (draft === this.boundDraft) return
    this.generation++
    const keepModels = sameServer(this.boundDraft, draft)
    this.boundDraft = draft
    this.publish({ models: keepModels ? this.state.models : null, loading: false, error: null })
  }
  find = async () => {
    this.bind()
    const draft = this.boundDraft
    if (!this.listeners.size || this.state.loading || draft?.connection.provider !== "ollama")
      return
    const generation = ++this.generation
    const isCurrent = () =>
      generation === this.generation && this.listeners.size > 0 && this.getDraft() === draft
    this.publish({ loading: true, error: null })
    try {
      const models = await getInstalledOllamaModels(draft)
      if (isCurrent()) this.publish({ models })
    } catch (error) {
      if (isCurrent())
        this.publish({
          error: providerErrorMessage(
            error,
            "Could not find installed models. Check the server URL and authentication, then try again.",
          ),
        })
    } finally {
      if (isCurrent()) this.publish({ loading: false })
    }
  }
}
