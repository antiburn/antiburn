export type UsageState = {
  accountRetryAt: number | null
  pendingRead: { generation: number; revision: number } | null
  snapshot: {
    status: "ready" | "empty" | "error"
    windows: {
      kind: string
      label: string
      percent: number | null
      resetsAt: string | null
      startsOnMessage?: boolean
    }[]
    capturedAt: number
  }
  revealed: string | null
  activated: string | null
  hovered: { target: string; requestId: string; binding: string } | null
}

declare module "claude-code" {
  interface PluginState {
    antiburn: { usage: UsageState }
  }
}

export type AntiburnDescription = {
  id: string
  version: string
  capabilities: { usage: boolean; usageUiEnabled: boolean }
}

declare module "claude-code" {
  interface EngineInterface {
    antiburn: { describe(): Promise<AntiburnDescription> }
  }
}
