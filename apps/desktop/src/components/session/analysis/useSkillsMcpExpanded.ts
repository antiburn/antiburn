import { useSyncExternalStore } from "react"

import { createExternalStore } from "../../../lib/externalStore"
import { DEFAULT_SETTINGS, getSettings, onSettingsChanged, setSettings } from "../../../lib/ipc"

// Only the newest optimistic write or shell event can replace the visible
// value. An older write that lands late has nothing left to say.
let revision = 0

/**
 * Whether the Skills & MCPs chart shows every row or only the first group.
 *
 * The reader's answer is one stored preference, not component state and not a
 * process-lifetime flag: it applies to every session, it survives a quit, and
 * it carries across an upgrade, because it lives in the same `setting` table
 * as every other preference. Two controls write it — the chart's own button
 * and the Appearance pane's switch — and nothing else changes it.
 *
 * The store reads the stored answer once and then follows the shell's
 * `settings:changed` broadcast, so the chart and the settings window agree
 * without either one knowing about the other.
 */
export const skillsMcpExpandedStore = createExternalStore<boolean>({
  initial: DEFAULT_SETTINGS.skillsMcpExpanded,
  load: async () => (await getSettings()).skillsMcpExpanded,
  subscribe: (set) =>
    onSettingsChanged((settings) => {
      revision += 1
      set(settings.skillsMcpExpanded)
    }),
})

/**
 * Write the reader's choice through to the store.
 *
 * Reads the rest of the preferences first so this write carries them
 * unchanged. A write that fails puts the previous value back, so the button
 * never keeps claiming a choice the store did not take.
 */
async function storeExpanded(
  expanded: boolean,
  previous: boolean,
  written: number,
): Promise<void> {
  try {
    const current = await getSettings()
    const saved = await setSettings({ ...current, skillsMcpExpanded: expanded })
    if (written !== revision) return
    skillsMcpExpandedStore.set(saved.skillsMcpExpanded)
  } catch {
    if (written !== revision) return
    skillsMcpExpandedStore.set(previous)
  }
}

function setExpanded(expanded: boolean): void {
  // Optimistic, the same way the settings panes write: the button must not lag
  // behind the pointer, and the stored answer replaces this one a moment later.
  const previous = skillsMcpExpandedStore.getSnapshot()
  if (previous === expanded) return
  const written = ++revision
  skillsMcpExpandedStore.set(expanded)
  void storeExpanded(expanded, previous, written)
}

export function useSkillsMcpExpanded(): [boolean, (expanded: boolean) => void] {
  const expanded = useSyncExternalStore(
    skillsMcpExpandedStore.subscribe,
    skillsMcpExpandedStore.getSnapshot,
  )
  return [expanded, setExpanded]
}
