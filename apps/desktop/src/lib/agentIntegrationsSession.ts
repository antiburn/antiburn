import {
  getAgentIntegrations,
  onAgentIntegrationsChanged,
  repairAgentIntegration,
  removeAgentIntegration,
  type IntegrationStatus,
  type IntegrationVendor,
} from "./agentIntegrationsIpc"
import { createExternalStore } from "./externalStore"

type OperationState = { busy: boolean; error: string | null }
type Snapshot = OperationState & { statuses: IntegrationStatus[] }

export class AgentIntegrationsSession {
  private operation = 0
  private subscribers = 0
  private readonly state = createExternalStore<OperationState>({
    initial: { busy: false, error: null },
  })
  private readonly statuses = createExternalStore<IntegrationStatus[]>({
    initial: [],
    load: () => this.request(getAgentIntegrations),
    subscribe: (set) => {
      this.operation += 1
      this.state.set({ busy: true, error: null })
      return onAgentIntegrationsChanged(set)
    },
  })
  private snapshot: Snapshot = { statuses: [], busy: false, error: null }

  getSnapshot = (): Snapshot => {
    const statuses = this.statuses.getSnapshot()
    const state = this.state.getSnapshot()
    if (
      this.snapshot.statuses !== statuses ||
      this.snapshot.busy !== state.busy ||
      this.snapshot.error !== state.error
    )
      this.snapshot = { statuses, ...state }
    return this.snapshot
  }

  subscribe = (listener: () => void): (() => void) => {
    this.subscribers += 1
    const stopState = this.state.subscribe(listener)
    const stopStatuses = this.statuses.subscribe(listener)
    let subscribed = true
    return () => {
      if (!subscribed) return
      subscribed = false
      this.subscribers -= 1
      if (this.subscribers === 0) this.operation += 1
      stopStatuses()
      stopState()
    }
  }

  reportNavigationError = (): void =>
    this.state.set({
      ...this.state.getSnapshot(),
      error: "Could not open CLI settings. Try again from Settings.",
    })

  repair = (vendor: IntegrationVendor): Promise<void> =>
    this.run(() => repairAgentIntegration(vendor), true)
  remove = (vendor: IntegrationVendor): Promise<void> =>
    this.run(() => removeAgentIntegration(vendor), true)
  refresh = (): Promise<void> => this.run(getAgentIntegrations)

  private async run(
    action: () => Promise<IntegrationStatus[]>,
    reconcileOnError = false,
  ): Promise<void> {
    if (this.state.getSnapshot().busy) return
    const operation = this.operation + 1
    await this.request(async () => {
      try {
        await this.statuses.refresh(action)
      } catch (error) {
        if (reconcileOnError && operation === this.operation)
          await this.statuses.refresh(getAgentIntegrations).catch(() => undefined)
        throw error
      }
    }).catch(() => undefined)
  }

  private async request<T>(action: () => Promise<T>): Promise<T> {
    const operation = ++this.operation
    this.state.set({ busy: true, error: null })
    try {
      return await action()
    } catch (error) {
      if (operation === this.operation) {
        this.state.set({
          busy: true,
          error: "Could not update agent integrations. Try again.",
        })
      }
      throw error
    } finally {
      if (operation === this.operation)
        this.state.set({ ...this.state.getSnapshot(), busy: false })
    }
  }
}

export const agentIntegrationsSession = new AgentIntegrationsSession()
