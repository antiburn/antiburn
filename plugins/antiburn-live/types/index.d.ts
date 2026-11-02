export type LiveUsageState = {
  layout?: 'inline' | 'current'
  preview?: { id: string; capturedAt: number } | null
  sequence?: string
  showSyntheticLabels?: boolean
  previewReveal?: 'show' | 'hide' | 'pace-show' | 'pace-hide'
  presentationSessionId?: string
  snapshot: {
    status: 'ready' | 'empty' | 'error'
    windows: {
      kind: string
      label: string
      percent: number | null
      resetsAt: string | null
    }[]
    capturedAt: number
  }
  revealed: string | null
  hovered?: { target: string; requestId: string; binding: string } | null
  activated: string | null
}

export type LiveCheckId = string

export type LiveStage = 'building' | 'active' | 'watching' | 'resolved' | 'quiet'

/** `advise` shows advice for the next request and sends nothing. */
export type LiveActionKind = 'submit' | 'compact' | 'fill' | 'advise'

export type LiveCheckView = {
  id: LiveCheckId
  label: string
  area: string
  sentence: string
  stage: LiveStage
  metric: string
  progress: string | null
  /** A cause line under the metric, such as E2 under Context heavy. */
  cause: string | null
  actionKind: LiveActionKind
  actionLabel: string
  prompt: string
  isHidden: boolean
  isStoppedAfterFix: boolean
  source?: 'session'
  checking?: boolean
  assessedAt?: number | null
}

export type LiveViewState = {
  checks: LiveCheckView[]
  /** The checks that the resolved strip names for one turn. */
  resolvedLine: LiveCheckId[]
  /** The stages that took a new check at the last publish. Their section icons draw in once. */
  entered: LiveStage[]
  connection?: 'connected' | 'waiting' | 'disconnected' | 'unavailable' | 'inactive'
  freshnessEvaluated?: boolean
  newSessionCount?: number
}

export type LiveStripState = {
  isDismissed: boolean
  newCount: number
  armedId: LiveCheckId | null
}

export type LivePaneState = {
  selectedId: LiveCheckId | null
  /** Titles of the folded groups that the person opened. */
  openGroups: string[]
  /** The chevron that the person pressed last: a check id or a group title. Only this chevron turns. */
  turned: string | null
  /** The check whose prompt block shows in full. Other blocks show about three lines. */
  fullPromptId: LiveCheckId | null
}

export type AntiburnDescription = {
  id: string
  version: string
  capabilities: { usage: boolean; usageUiEnabled: boolean }
}

declare module 'claude-code' {
  interface EngineInterface {
    antiburn: { describe(): Promise<AntiburnDescription> }
    antiburnDevelopment: {
      describe(): Promise<{
        id: string
        capabilities: { liveChecks: boolean }
      }>
    }
  }
}

declare module 'claude-code' {
  interface PluginState {
    'antiburn-live': {
      view: LiveViewState
      strip: LiveStripState
      pane: LivePaneState
      usage: LiveUsageState
      burn: LiveBurnState
    }
  }
}

export type LiveBurnState = {
  active: boolean
  previewUntil: number
  reduced: boolean
  startedAt: number
  coolingAt: number | null
  revision: number
  estimatedTps: number
  measuredTps: number | null
  source: string
}
