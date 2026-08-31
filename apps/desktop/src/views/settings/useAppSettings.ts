import { useCallback, useSyncExternalStore } from "react"

import { applyTheme } from "../../lib/appearance"
import { createExternalStore } from "../../lib/externalStore"
import {
  DEFAULT_SETTINGS,
  getSettings,
  onSettingsChanged,
  setSettings,
  type AppSettings,
} from "../../lib/ipc"

/**
 * The settings window's copy of the reader's preferences.
 *
 * Writes go straight through — a settings pane with a Save button is a settings
 * pane that can be left in a lying state. The store returns what it actually
 * stored (clamped), and that is what the panes then render, so a value the
 * store refused can never linger on screen.
 *
 * The theme is applied as soon as it is known, and again on every change, so
 * this window follows the choice being made in it.
 */
export interface AppSettingsController {
  settings: AppSettings
  /** False until the first read resolves. */
  loaded: boolean
  /** Merge a change into the stored preferences. */
  update: (change: Partial<AppSettings>) => Promise<void>
}

type SettingsSnapshot = { settings: AppSettings; loaded: boolean }

// Only the newest optimistic write or shell event can replace the visible settings.
let settingsRevision = 0

// Module-level: every window that mounts this hook shares one read and one
// subscription to the broadcast, rather than each racing its own.
const settingsStore = createExternalStore<SettingsSnapshot>({
  initial: { settings: DEFAULT_SETTINGS, loaded: false },
  load: async () => {
    try {
      const stored = await getSettings()
      applyTheme(stored.theme)
      return { settings: stored, loaded: true }
    } catch {
      return { settings: DEFAULT_SETTINGS, loaded: true }
    }
  },
  // Writes can come from another window (onboarding and the popover share the
  // same settings store); the broadcast keeps this window's copy — and its
  // theme — current too.
  subscribe: (set) =>
    onSettingsChanged((stored) => {
      settingsRevision += 1
      applyTheme(stored.theme)
      set({ settings: stored, loaded: true })
    }),
})

export function useAppSettings(): AppSettingsController {
  const { settings, loaded } = useSyncExternalStore(
    settingsStore.subscribe,
    settingsStore.getSnapshot,
  )

  const update = useCallback(async (change: Partial<AppSettings>) => {
    // Optimistic, so a switch does not lag behind the pointer; the stored
    // answer replaces it a moment later and wins any disagreement. Reads the
    // current value from the store rather than closing over it, so this
    // callback stays referentially stable across every settings change.
    const current = settingsStore.getSnapshot().settings
    const next = { ...current, ...change }
    const revision = ++settingsRevision
    applyTheme(next.theme)
    settingsStore.set({ settings: next, loaded: true })
    try {
      const saved = await setSettings(next)
      if (revision !== settingsRevision) return
      applyTheme(saved.theme)
      settingsStore.set({ settings: saved, loaded: true })
    } catch {
      if (revision !== settingsRevision) return
      applyTheme(current.theme)
      settingsStore.set({ settings: current, loaded: true })
    }
  }, [])

  return { settings, loaded, update }
}
