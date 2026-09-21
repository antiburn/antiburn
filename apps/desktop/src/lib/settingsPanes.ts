/**
 * The settings window's section ids.
 *
 * They live here rather than in `SettingsView` because two other places name
 * one: the IPC wrapper that asks the shell to open a particular pane, and the
 * popover's attention banners that send a reader to it. A shared union means a
 * renamed pane is a type error at every call site instead of a link that
 * quietly lands nowhere.
 */
export const SETTINGS_PANES = [
  { id: "general", label: "General" },
  { id: "sources", label: "Sources" },
  { id: "notifications", label: "Notifications" },
  { id: "usage", label: "Usage" },
  { id: "appearance", label: "Appearance" },
  { id: "privacy", label: "Privacy" },
  { id: "about", label: "About" },
] as const

export type SettingsPane = (typeof SETTINGS_PANES)[number]["id"]

export function settingsPaneLabel(id: SettingsPane): string {
  return SETTINGS_PANES.find((pane) => pane.id === id)!.label
}

/** Whether an arbitrary value — a shell payload, say — names a real pane. */
export function isSettingsPane(value: unknown): value is SettingsPane {
  return typeof value === "string" && SETTINGS_PANES.some((pane) => pane.id === value)
}
