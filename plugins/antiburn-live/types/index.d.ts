export type LiveCheckId = 'X1' | 'E1' | 'D' | 'O' | 'T' | 'K1' | 'C1' | 'K2'

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
}

export type LiveViewState = {
  checks: LiveCheckView[]
  /** The checks that the resolved strip names for one turn. */
  resolvedLine: LiveCheckId[]
  /** The stages that took a new check at the last publish. Their section icons draw in once. */
  entered: LiveStage[]
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

declare module 'claude-code' {
  interface PluginState {
    'antiburn-live': {
      view: LiveViewState
      strip: LiveStripState
      pane: LivePaneState
    }
  }
}
