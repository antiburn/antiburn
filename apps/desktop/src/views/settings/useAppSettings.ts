import { useCallback, useSyncExternalStore } from "react"

import { applyTheme } from "../../lib/appearance"
import { createExternalStore } from "../../lib/externalStore"
import {
  DEFAULT_SETTINGS,
  getSettings,
  onSettingsChanged,
  setSettings,
  patchSidebarUsage,
  type SidebarUsagePatch,
  setInterfaceScale,
  startLiveUsage,
  type AppSettings,
} from "../../lib/ipc"
import type { InterfaceScaleChange } from "../../lib/interfaceScale"

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
  /** False until saved settings are available. */
  loaded: boolean
  /** Merge a change into the stored preferences. */
  update: (change: SettingsChange) => Promise<void>
}

type SettingsSnapshot = { settings: AppSettings; loaded: boolean }
type SettingsChange = Partial<Omit<AppSettings, "interfaceScalePercent">>

// Only the newest optimistic write or shell event can replace the visible settings.
let settingsRevision = 0

// Module-level: every window that mounts this hook shares one read and one
// subscription to the broadcast, rather than each racing its own.
const settingsStore = createExternalStore<SettingsSnapshot>({
  initial: { settings: DEFAULT_SETTINGS, loaded: false },
  resetOnStop: true,
  load: async () => {
    try {
      const stored = await getSettings()
      applyTheme(stored.theme)
      return { settings: stored, loaded: true }
    } catch {
      return { settings: DEFAULT_SETTINGS, loaded: false }
    }
  },
  // Writes can come from another window (the main window and the popover
  // share the same settings store); the broadcast keeps this window's copy —
  // and its theme — current too.
  subscribe: async (set) => {
    const stop = await onSettingsChanged((stored) => {
      settingsRevision += 1
      applyTheme(stored.theme)
      set({ settings: stored, loaded: true })
    }).catch(() => null)
    const retry = () => {
      if (!settingsStore.getSnapshot().loaded) void settingsStore.refresh()
    }
    window.addEventListener("focus", retry)
    return () => {
      window.removeEventListener("focus", retry)
      stop?.()
    }
  },
})

/** Save scale through its atomic command and publish the confirmed preference. */
export async function saveInterfaceScale(change: InterfaceScaleChange): Promise<void> {
  const revision = ++settingsRevision
  const saved = await setInterfaceScale(change, "settings")
  if (revision === settingsRevision) settingsStore.set({ settings: saved, loaded: true })
}

/** Publish activation only after the native settings transaction succeeds. */
export async function enableLiveUsage(): Promise<void> {
  const revision = ++settingsRevision
  const saved = await startLiveUsage()
  if (revision === settingsRevision) settingsStore.set({ settings: saved, loaded: true })
}

export function useAppSettings(): AppSettingsController & {
  updateSidebar: (change: SidebarUsagePatch) => Promise<void>
} {
  const { settings, loaded } = useSyncExternalStore(
    settingsStore.subscribe,
    settingsStore.getSnapshot,
  )

  const save = useCallback(
    async (change: SettingsChange, persist: (next: AppSettings) => Promise<AppSettings>) => {
      if (!settingsStore.getSnapshot().loaded) return
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
        const saved = await persist(next)
        if (revision !== settingsRevision) return
        applyTheme(saved.theme)
        settingsStore.set({ settings: saved, loaded: true })
      } catch {
        if (revision !== settingsRevision) return
        applyTheme(current.theme)
        settingsStore.set({ settings: current, loaded: true })
      }
    },
    [],
  )

  const update = useCallback((change: SettingsChange) => save(change, setSettings), [save])
  const updateSidebar = useCallback(
    (change: SidebarUsagePatch) => save(change, (next) => patchSidebarUsage(change, next)),
    [save],
  )

  return { settings, loaded, update, updateSidebar }
}
