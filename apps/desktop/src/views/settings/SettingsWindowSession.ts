import { getCurrentWindow } from "@tauri-apps/api/window"

import {
  appInfo,
  hasShell,
  noteInteraction,
  onSessionsInvalidated,
  onSettingsPaneRequest,
  onSettingsShown,
  takeSettingsPane,
  type AppInfo,
} from "../../lib/ipc"
import { isSettingsPane, type SettingsPane } from "../../lib/settingsPanes"

export type SettingsWindowSnapshot = {
  info: AppInfo | null
  pane: SettingsPane
  visible: boolean
}

/**
 * The imperative boundary between the settings window and the shell.
 *
 * React reads immutable snapshots through `useSyncExternalStore`; the
 * `app_info` fetch, the requested-pane handshake, and the subscription that
 * moves an already-open window stay here, where they belong to the external
 * systems that created them rather than to a component lifecycle. See
 * `OnboardingSession` for the same shape applied to a different window.
 */
export class SettingsWindowSession {
  private listeners = new Set<() => void>()
  private started = false
  private generation = 0
  private infoRevision = 0
  private paneEventRevision = 0
  private visibilityRevision = 0
  private paneResolved = false
  private exposedPane: SettingsPane | null = null
  private stopInvalidationListening: (() => void) | null = null
  private stopPaneListening: (() => void) | null = null
  private stopShownListening: (() => void) | null = null

  private snapshot: SettingsWindowSnapshot = { info: null, pane: "general", visible: false }

  getSnapshot = (): SettingsWindowSnapshot => this.snapshot

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    if (!this.started) void this.start()
    return () => {
      this.listeners.delete(listener)
      if (this.listeners.size === 0) this.stop()
    }
  }

  /** The sidebar's `onChange`. */
  setPane = (pane: SettingsPane): void => {
    this.paneEventRevision += 1
    this.paneResolved = true
    this.selectVisiblePane(pane)
  }

  private start = async (): Promise<void> => {
    this.started = true
    const generation = ++this.generation
    const paneEventRevision = this.paneEventRevision
    document.addEventListener("visibilitychange", this.handleDocumentVisibility)

    this.refreshInfo(generation)

    void onSessionsInvalidated(() => {
      if (generation !== this.generation) return
      this.refreshInfo(generation)
    })
      .then((stop) => {
        if (generation !== this.generation) {
          stop()
          return
        }
        this.stopInvalidationListening = stop
      })
      .catch(() => {})

    const stopShown = await onSettingsShown(() => {
      if (generation !== this.generation) return
      this.visibilityRevision += 1
      this.markVisible()
    }).catch(() => null)
    if (generation !== this.generation) {
      stopShown?.()
      return
    }
    this.stopShownListening = stopShown

    if (!hasShell()) {
      this.markVisible()
    }

    const visibilityRevision = this.visibilityRevision
    if (hasShell()) {
      void Promise.resolve()
        .then(() => getCurrentWindow().isVisible())
        .then((visible) => {
          if (generation !== this.generation || visibilityRevision !== this.visibilityRevision)
            return
          if (visible) this.markVisible()
        })
        .catch(() => {})
    }

    const stop = await onSettingsPaneRequest((requested) => {
      if (generation !== this.generation) return
      this.paneEventRevision += 1
      void takeSettingsPane().catch(() => {})
      if (isSettingsPane(requested)) {
        this.paneResolved = true
        this.selectVisiblePane(requested)
      }
    }).catch(() => null)
    if (generation !== this.generation) {
      stop?.()
      return
    }
    this.stopPaneListening = stop

    void takeSettingsPane()
      .then((requested) => {
        if (generation !== this.generation) return
        if (paneEventRevision !== this.paneEventRevision) return
        this.paneResolved = true
        this.selectVisiblePane(isSettingsPane(requested) ? requested : this.snapshot.pane)
      })
      .catch(() => {
        if (generation !== this.generation) return
        if (paneEventRevision !== this.paneEventRevision) return
        this.paneResolved = true
        this.selectVisiblePane(this.snapshot.pane)
      })
  }

  private stop(): void {
    this.started = false
    this.generation += 1
    document.removeEventListener("visibilitychange", this.handleDocumentVisibility)
    this.stopInvalidationListening?.()
    this.stopInvalidationListening = null
    this.stopPaneListening?.()
    this.stopPaneListening = null
    this.stopShownListening?.()
    this.stopShownListening = null
  }

  private refreshInfo(generation: number): void {
    const revision = ++this.infoRevision
    void appInfo()
      .then((info) => {
        if (generation !== this.generation || revision !== this.infoRevision) return
        this.update({ info })
      })
      .catch(() => {
        if (generation !== this.generation || revision !== this.infoRevision) return
        this.update({ info: null })
      })
  }

  private update(change: Partial<SettingsWindowSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...change }
    for (const listener of this.listeners) listener()
  }

  private selectVisiblePane(pane: SettingsPane): void {
    if (pane !== this.snapshot.pane) this.update({ pane })
    if (!this.started || !this.paneResolved || !this.snapshot.visible) return
    if (pane === this.exposedPane) return
    this.exposedPane = pane
    noteInteraction({ kind: "settingsPaneViewed", pane })
  }

  private markVisible(): void {
    if (this.snapshot.visible) return
    this.exposedPane = null
    this.update({ visible: true })
    if (this.paneResolved) this.selectVisiblePane(this.snapshot.pane)
  }

  private handleDocumentVisibility = (): void => {
    if (document.visibilityState !== "hidden" || !this.snapshot.visible) return
    this.visibilityRevision += 1
    this.update({ visible: false })
  }
}
