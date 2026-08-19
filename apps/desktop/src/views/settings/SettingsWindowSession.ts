// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { appInfo, onSettingsPaneRequest, takeSettingsPane, type AppInfo } from "../../lib/ipc"
import { isSettingsPane, type SettingsPane } from "../../lib/settingsPanes"

export type SettingsWindowSnapshot = {
  info: AppInfo | null
  pane: SettingsPane
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
  private stopPaneListening: (() => void) | null = null

  private snapshot: SettingsWindowSnapshot = { info: null, pane: "general" }

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
    this.update({ pane })
  }

  private start = async (): Promise<void> => {
    this.started = true
    const generation = ++this.generation

    void appInfo()
      .then((info) => {
        if (generation !== this.generation) return
        this.update({ info })
      })
      .catch(() => {
        if (generation !== this.generation) return
        this.update({ info: null })
      })

    // A pane somebody asked for. Taken once on start (the window may have been
    // created *by* that request), and listened for afterwards (a window that was
    // already open never mounts again). Unknown ids are ignored rather than
    // rendering nothing. `takeSettingsPane` is called before `onSettingsPaneRequest`
    // is awaited below, matching the relative ordering the original effects ran in.
    void takeSettingsPane()
      .then((requested) => {
        if (generation !== this.generation) return
        if (isSettingsPane(requested)) this.update({ pane: requested })
      })
      .catch(() => {})

    const stop = await onSettingsPaneRequest((requested) => {
      if (generation !== this.generation) return
      if (isSettingsPane(requested)) this.update({ pane: requested })
    })
    if (generation !== this.generation) {
      stop()
      return
    }
    this.stopPaneListening = stop
  }

  private stop(): void {
    this.started = false
    this.generation += 1
    this.stopPaneListening?.()
    this.stopPaneListening = null
  }

  private update(change: Partial<SettingsWindowSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...change }
    for (const listener of this.listeners) listener()
  }
}
